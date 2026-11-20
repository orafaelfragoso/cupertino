//! What the session menu offers, from logind's answers and the session manager, and the low-battery notices.

use cupertino_shell_agent::capabilities::{Answer, Capabilities, Inputs};
use cupertino_shell_agent::low_battery::{Action, Level, LowBattery};

#[test]
fn logind_answers_decide_the_power_items_and_the_manager_the_switch() {
    assert_eq!(Answer::from_logind("challenge"), Answer::Challenge);
    assert_eq!(Answer::from_logind("maybe"), Answer::NotAvailable);
    let inputs = Inputs {
        can_suspend: Answer::Yes,
        can_reboot: Answer::Challenge,
        can_power_off: Answer::No,
        session_manager: false,
    };
    let caps = Capabilities::new(&inputs);
    assert!(caps.sleep && caps.restart && !caps.shut_down);
    assert!(caps.log_out && caps.lock && !caps.switch_user);
    assert_eq!(
        Capabilities::reasons(&inputs),
        vec![
            "shut-down: logind answers No".to_string(),
            "switch-user: no session manager on the system bus".to_string()
        ]
    );
    let with_manager = Inputs {
        session_manager: true,
        ..inputs
    };
    assert!(Capabilities::new(&with_manager).switch_user);
}

#[test]
fn a_rising_level_posts_replacing_the_last_and_none_withdraws_it() {
    let mut battery = LowBattery::new();
    assert_eq!(battery.report(Level::None, 80.0), None);
    let Some(Action::Post { replaces, notice }) = battery.report(Level::from_upower(3), 9.6) else {
        panic!("a low level posts");
    };
    assert_eq!(replaces, 0);
    assert_eq!(notice.urgency, 1);
    assert!(
        notice.body.starts_with("10% of the battery remains."),
        "{}",
        notice.body
    );
    battery.posted(7);
    assert_eq!(battery.report(Level::Low, 8.0), None);
    let Some(Action::Post { replaces, notice }) = battery.report(Level::from_upower(4), 4.0) else {
        panic!("a critical level posts");
    };
    assert_eq!((replaces, notice.urgency), (7, 2));
    battery.posted(8);
    assert_eq!(
        battery.report(Level::from_upower(1), 4.0),
        Some(Action::Close(8))
    );
    assert_eq!(battery.report(Level::None, 4.0), None);
}
