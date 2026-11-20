//! The confirmation panels, the power-button panel and the interrupt panel: their keys, countdown, words and place.

use cupertino_shell_agent::interrupt::{self, Choice, Reason, Stage};
use cupertino_shell_agent::panel::{
    format_countdown, Action, Decision, Key, Kind, Panel, Placement, COUNTDOWN_SECONDS,
    NO_BREAK_SPACE,
};

#[test]
fn return_does_the_default_and_escape_cancels() {
    for (kind, action) in [
        (Kind::LogOut, Action::LogOut),
        (Kind::Restart, Action::Restart),
        (Kind::ShutDown, Action::ShutDown),
        (Kind::PowerButton, Action::ShutDown),
    ] {
        let panel = Panel::new(kind, 0.0, true);
        assert_eq!(panel.press(Key::Return), Some(Decision::Do(action)));
        assert_eq!(panel.press(Key::Escape), Some(Decision::Cancel));
        assert_eq!(panel.press(Key::Backspace), None);
    }
}

#[test]
fn the_power_button_panel_takes_r_and_s_and_the_others_do_not() {
    let power = Panel::new(Kind::PowerButton, 0.0, true);
    assert_eq!(
        power.press(Key::Char('r')),
        Some(Decision::Do(Action::Restart))
    );
    assert_eq!(
        power.press(Key::Char('S')),
        Some(Decision::Do(Action::Sleep))
    );
    assert_eq!(power.press(Key::Char('x')), None);
    let log_out = Panel::new(Kind::LogOut, 0.0, true);
    assert_eq!(log_out.press(Key::Char('r')), None);
}

#[test]
fn the_countdown_ends_in_the_default_action_and_the_power_button_panel_waits() {
    let panel = Panel::new(Kind::Restart, 10.0, true);
    assert_eq!(panel.seconds_left(10.0), Some(60));
    assert_eq!(panel.seconds_left(10.5), Some(60));
    assert_eq!(panel.seconds_left(69.5), Some(1));
    assert_eq!(panel.tick(10.0 + COUNTDOWN_SECONDS - 0.01), None);
    assert_eq!(
        panel.tick(10.0 + COUNTDOWN_SECONDS),
        Some(Decision::Do(Action::Restart))
    );
    assert_eq!(panel.seconds_left(1000.0), Some(0));
    let wake = panel.next_wakeup(10.25).expect("a countdown wakes");
    assert!(
        (wake - 0.75).abs() < 1e-9,
        "the next number shows in 0.75 s, not {wake}"
    );

    let power = Panel::new(Kind::PowerButton, 0.0, true);
    assert_eq!(power.seconds_left(0.0), None);
    assert_eq!(power.tick(1000.0), None);
    assert_eq!(power.next_wakeup(0.0), None);
}

#[test]
fn the_checkbox_starts_at_the_choice_and_space_flips_it() {
    let mut panel = Panel::new(Kind::LogOut, 0.0, false);
    assert!(!panel.reopen());
    assert!(panel.toggles(Key::Char(' ')));
    assert!(!panel.toggles(Key::Return));
    panel.toggle_reopen();
    assert!(panel.reopen());
}

#[test]
fn the_countdown_reads_in_seconds_up_to_a_minute_then_in_full_units() {
    let nb = NO_BREAK_SPACE;
    assert_eq!(format_countdown(60), format!("60{nb}seconds"));
    assert_eq!(format_countdown(1), format!("1{nb}second"));
    assert_eq!(format_countdown(61), format!("1{nb}minute, 1{nb}second"));
    assert_eq!(format_countdown(120), format!("2{nb}minutes"));
    assert_eq!(
        Kind::LogOut.countdown_message(42),
        Some(format!("Logging out in 42{nb}seconds."))
    );
    assert_eq!(Kind::PowerButton.countdown_message(42), None);
    assert_eq!(Kind::PowerButton.text().title, Kind::ShutDown.text().title);
}

#[test]
fn a_panel_is_centred_and_kept_on_its_output_while_it_moves() {
    let mut placement = Placement::centred((1000.0, 800.0), (300.0, 200.0));
    assert_eq!(placement.origin(), (350, 300));
    placement.moved((-1000.0, 10.4));
    assert_eq!(placement.origin(), (0, 310));
    placement.moved((5000.0, 5000.0));
    assert_eq!(placement.origin(), (700, 600));
}

#[test]
fn the_interrupt_panel_is_worded_by_its_reason_and_answers_its_keys() {
    assert_eq!(Reason::of_status("kept-windows"), Reason::Cancelled);
    assert_eq!(Reason::of_status("not-responding"), Reason::NotResponding);
    assert_eq!(Reason::of_status("closing"), Reason::FailedToQuit);
    assert!(!Reason::Cancelled.offers_force_quit());
    assert!(Reason::NotResponding.offers_force_quit());

    let title = interrupt::title(Reason::NotResponding, Stage::Ask, Action::Restart, "Editor");
    assert_eq!(
        title,
        "The restart stopped because \u{201c}Editor\u{201d} is not responding."
    );
    let confirm = interrupt::title(
        Reason::FailedToQuit,
        Stage::ConfirmForceQuit,
        Action::LogOut,
        "Editor",
    );
    assert_eq!(confirm, "Force \u{201c}Editor\u{201d} to quit?");

    assert_eq!(
        interrupt::press(Stage::Ask, Key::Return),
        Some(Choice::TryAgain)
    );
    assert_eq!(
        interrupt::press(Stage::Ask, Key::Escape),
        Some(Choice::Cancel)
    );
    assert_eq!(
        interrupt::press(Stage::ConfirmForceQuit, Key::Escape),
        Some(Choice::Back)
    );
    assert_eq!(interrupt::press(Stage::ConfirmForceQuit, Key::Return), None);
}

#[test]
fn the_interrupt_panel_offers_force_quit_only_for_its_reasons() {
    let labels = |reason| {
        interrupt::Frames::new(reason, Stage::Ask, 1, 1)
            .buttons
            .iter()
            .map(|b| b.choice)
            .collect::<Vec<_>>()
    };
    assert!(!labels(Reason::Cancelled).contains(&Choice::ForceQuit));
    assert!(labels(Reason::NotResponding).contains(&Choice::ForceQuit));
    assert!(labels(Reason::FailedToQuit).contains(&Choice::TryAgain));
}
