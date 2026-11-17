//! The device and session bookkeeping, driven by the messages logind sends.

use cupertino_devices::DevT;
use cupertino_seat::state::{Input, Machine, Output, PauseKind, Released, Transition};
use cupertino_seat::Error;

fn gpu() -> DevT {
    DevT::new(226, 0)
}

fn keyboard() -> DevT {
    DevT::new(13, 64)
}

fn pause(dev: DevT) -> Input {
    Input::PauseDevice {
        dev,
        kind: PauseKind::Pause,
    }
}

fn force(dev: DevT) -> Input {
    Input::PauseDevice {
        dev,
        kind: PauseKind::Force,
    }
}

fn gone(dev: DevT) -> Input {
    Input::PauseDevice {
        dev,
        kind: PauseKind::Gone,
    }
}

fn resume(dev: DevT) -> Input {
    Input::ResumeDevice { dev }
}

/// A running session holding the GPU as descriptor 10 and the keyboard as descriptor 11.
fn running() -> Machine {
    let mut machine = Machine::new(true);
    machine.take(gpu(), 10, false).unwrap();
    machine.take(keyboard(), 11, false).unwrap();
    machine
}

const PAUSED: Output = Output::Transition(Transition::Pause);
const ACTIVATED: Output = Output::Transition(Transition::Activate);

#[test]
fn pause_kinds_parse_from_the_wire_strings() {
    assert_eq!(PauseKind::parse("pause").unwrap(), PauseKind::Pause);
    assert_eq!(PauseKind::parse("force").unwrap(), PauseKind::Force);
    assert_eq!(PauseKind::parse("gone").unwrap(), PauseKind::Gone);
    assert!(matches!(
        PauseKind::parse("Pause"),
        Err(Error::UnknownPauseType(kind)) if kind == "Pause"
    ));
    assert!(PauseKind::parse("").is_err());
}

#[test]
fn a_session_runs_when_it_starts_active() {
    assert!(Machine::new(true).is_running());
    assert!(!Machine::new(true).is_waiting_to_activate());
    assert!(!Machine::new(false).is_running());
    assert!(Machine::new(false).is_waiting_to_activate());
}

#[test]
fn a_device_cannot_be_taken_twice() {
    let mut machine = running();
    assert!(matches!(
        machine.take(gpu(), 12, false),
        Err(Error::AlreadyTaken(dev)) if dev == gpu()
    ));
}

#[test]
fn the_first_pause_of_any_device_pauses_the_session_and_asks_for_the_acknowledgement() {
    let mut machine = running();
    assert_eq!(
        machine.step(pause(gpu())),
        [PAUSED, Output::CompletePause(gpu())]
    );
    assert!(!machine.is_running());
    // the session is reported paused once: the second device only asks for its acknowledgement
    assert_eq!(
        machine.step(pause(keyboard())),
        [Output::CompletePause(keyboard())]
    );
}

#[test]
fn an_acknowledgement_is_due_once_and_only_while_the_device_is_held() {
    let mut machine = running();
    machine.step(pause(gpu()));
    assert!(machine.ack_pause(gpu()));
    assert!(!machine.ack_pause(gpu()), "already answered");
    assert!(!machine.ack_pause(keyboard()), "never asked");
    assert!(!machine.ack_pause(DevT::new(1, 1)), "not held");

    machine.step(pause(keyboard()));
    machine.release(11).unwrap();
    assert!(!machine.ack_pause(keyboard()), "released since");
}

#[test]
fn a_forced_pause_needs_no_acknowledgement() {
    let mut machine = running();
    assert_eq!(machine.step(force(gpu())), [PAUSED]);
    assert!(!machine.ack_pause(gpu()));
}

#[test]
fn the_session_activates_when_active_returns_and_every_device_resumed() {
    let mut machine = running();
    machine.step(pause(gpu()));
    machine.step(pause(keyboard()));
    assert_eq!(machine.step(Input::ActiveChanged(false)), []);
    assert!(machine.is_waiting_to_activate());

    assert_eq!(machine.step(Input::ActiveChanged(true)), []);
    assert_eq!(machine.step(resume(gpu())), []);
    assert_eq!(machine.step(resume(keyboard())), [ACTIVATED]);
    assert!(machine.is_running());
}

#[test]
fn a_resume_before_active_changes_activates_a_session_that_never_saw_it_turn_false() {
    let mut machine = running();
    machine.step(pause(gpu()));
    assert_eq!(machine.step(resume(gpu())), [ACTIVATED]);
    assert!(machine.is_running());
}

#[test]
fn a_property_change_alone_does_not_activate_a_session_that_is_only_pausing() {
    let mut machine = running();
    machine.step(pause(gpu()));
    assert_eq!(machine.step(Input::ActiveChanged(true)), []);
    assert!(!machine.is_running());
}

#[test]
fn active_turning_false_pauses_the_session_once() {
    let mut machine = running();
    assert_eq!(machine.step(Input::ActiveChanged(false)), [PAUSED]);
    assert_eq!(machine.step(Input::ActiveChanged(false)), []);
}

#[test]
fn a_session_without_devices_activates_when_active_returns() {
    let mut machine = Machine::new(false);
    assert_eq!(machine.step(Input::ActiveChanged(true)), [ACTIVATED]);
    assert!(machine.is_running());
    assert_eq!(machine.step(Input::ActiveChanged(true)), []);
}

#[test]
fn a_session_that_started_inactive_waits_for_its_devices_too() {
    let mut machine = Machine::new(false);
    machine.take(gpu(), 10, true).unwrap();
    assert_eq!(machine.step(Input::ActiveChanged(true)), []);
    assert_eq!(machine.step(resume(gpu())), [ACTIVATED]);
}

#[test]
fn a_device_taken_while_the_session_is_inactive_counts_as_paused_until_it_resumes() {
    let mut machine = Machine::new(true);
    machine.take(gpu(), 10, true).unwrap();
    machine.step(Input::ActiveChanged(false));
    machine.step(Input::ActiveChanged(true));
    assert!(!machine.is_running());
    assert_eq!(machine.step(resume(gpu())), [ACTIVATED]);
}

#[test]
fn a_gone_device_does_not_hold_the_session_back_and_ignores_later_signals() {
    let mut machine = running();
    assert_eq!(machine.step(gone(gpu())), []);
    assert_eq!(machine.step(pause(gpu())), []);
    assert_eq!(machine.step(resume(gpu())), []);
    assert_eq!(machine.step(Input::ActiveChanged(false)), [PAUSED]);
    assert_eq!(machine.step(Input::ActiveChanged(true)), [ACTIVATED]);
}

#[test]
fn signals_about_a_device_that_is_not_held_are_ignored() {
    let mut machine = running();
    let other = DevT::new(99, 9);
    assert_eq!(machine.step(pause(other)), []);
    assert_eq!(machine.step(force(other)), []);
    assert_eq!(machine.step(gone(other)), []);
    assert_eq!(machine.step(resume(other)), []);
    assert!(machine.is_running());
}

#[test]
fn releasing_a_device_finds_it_by_descriptor_and_asks_logind_to_let_go() {
    let mut machine = running();
    assert_eq!(
        machine.release(10).unwrap(),
        Released {
            dev: gpu(),
            notify_logind: true
        }
    );
    assert!(machine.is_unreleased(gpu()));
    machine.confirm_release(gpu());
    assert!(!machine.is_unreleased(gpu()));
}

#[test]
fn a_descriptor_not_handed_out_or_already_released_is_unknown() {
    let mut machine = running();
    assert!(matches!(machine.release(99), Err(Error::UnknownFd(99))));
    machine.release(10).unwrap();
    assert!(matches!(machine.release(10), Err(Error::UnknownFd(10))));
}

#[test]
fn releasing_a_gone_device_needs_no_call_to_logind() {
    let mut machine = running();
    machine.step(gone(gpu()));
    assert_eq!(
        machine.release(10).unwrap(),
        Released {
            dev: gpu(),
            notify_logind: false
        }
    );
    assert!(!machine.is_unreleased(gpu()));
}

#[test]
fn a_device_that_went_away_can_be_taken_again_under_a_new_descriptor() {
    let mut machine = running();
    machine.step(gone(gpu()));
    machine.take(gpu(), 20, false).unwrap();
    // the descriptor of the replaced entry is no longer recognised
    assert!(matches!(machine.release(10), Err(Error::UnknownFd(10))));
    assert_eq!(machine.release(20).unwrap().dev, gpu());
}

#[test]
fn a_released_paused_device_no_longer_holds_the_session_back() {
    let mut machine = running();
    machine.step(Input::ActiveChanged(false));
    machine.step(pause(gpu()));
    machine.step(Input::ActiveChanged(true));
    assert!(!machine.is_running());
    assert_eq!(machine.step(resume(keyboard())), []);
    machine.release(10).unwrap();
    assert!(machine.is_waiting_to_activate());
    assert_eq!(machine.step(Input::Recheck), [ACTIVATED]);
    assert!(machine.is_running());
}

#[test]
fn a_recheck_changes_nothing_while_devices_wait_or_active_is_false() {
    let mut machine = running();
    machine.step(Input::ActiveChanged(false));
    assert_eq!(machine.step(Input::Recheck), []);
    machine.step(Input::ActiveChanged(true));
    assert!(machine.is_running());
    assert_eq!(machine.step(Input::Recheck), []);
}

#[test]
fn a_released_device_does_not_start_a_pausing_session_again() {
    let mut machine = running();
    machine.step(pause(gpu()));
    machine.release(10).unwrap();
    // never saw Active turn false and no resume came: a recheck is not enough
    assert_eq!(machine.step(Input::Recheck), []);
    assert!(!machine.is_running());
}
