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

fn other() -> DevT {
    DevT::new(99, 9)
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

/// One step of a scenario: an input and what it must give, or a call on the machine.
enum Step {
    Step(Input, Vec<Output>),
    Take(DevT, i32, bool),
    Release(i32),
    Running(bool),
}

use Step::{Release, Running, Take};

fn step(input: Input, outputs: impl Into<Vec<Output>>) -> Step {
    Step::Step(input, outputs.into())
}

#[test]
fn a_session_runs_when_it_starts_active_and_otherwise_waits_to_activate() {
    for active in [true, false] {
        let machine = Machine::new(active);
        assert_eq!(machine.is_running(), active, "{active}");
        assert_eq!(machine.is_waiting_to_activate(), !active, "{active}");
    }
}

#[test]
fn the_session_pauses_once_and_activates_only_when_active_and_every_held_device_resumed() {
    let scenarios: Vec<(&str, Machine, Vec<Step>)> = vec![
        (
            "the first pause pauses and each pause asks for its acknowledgement",
            running(),
            vec![
                step(pause(gpu()), [PAUSED, Output::CompletePause(gpu())]),
                Running(false),
                step(pause(keyboard()), [Output::CompletePause(keyboard())]),
            ],
        ),
        (
            "a forced pause needs no acknowledgement",
            running(),
            vec![step(force(gpu()), [PAUSED])],
        ),
        (
            "active returns and every device resumes",
            running(),
            vec![
                step(pause(gpu()), [PAUSED, Output::CompletePause(gpu())]),
                step(pause(keyboard()), [Output::CompletePause(keyboard())]),
                step(Input::ActiveChanged(false), []),
                step(Input::ActiveChanged(true), []),
                step(resume(gpu()), []),
                step(resume(keyboard()), [ACTIVATED]),
                Running(true),
            ],
        ),
        (
            "a resume activates a session that never saw active turn false",
            running(),
            vec![
                step(pause(gpu()), [PAUSED, Output::CompletePause(gpu())]),
                step(resume(gpu()), [ACTIVATED]),
                Running(true),
            ],
        ),
        (
            "a property change alone does not activate a pausing session",
            running(),
            vec![
                step(pause(gpu()), [PAUSED, Output::CompletePause(gpu())]),
                step(Input::ActiveChanged(true), []),
                Running(false),
            ],
        ),
        (
            "active turning false pauses once",
            running(),
            vec![
                step(Input::ActiveChanged(false), [PAUSED]),
                step(Input::ActiveChanged(false), []),
            ],
        ),
        (
            "a session without devices activates when active returns",
            Machine::new(false),
            vec![
                step(Input::ActiveChanged(true), [ACTIVATED]),
                Running(true),
                step(Input::ActiveChanged(true), []),
            ],
        ),
        (
            "a session that started inactive waits for its devices",
            Machine::new(false),
            vec![
                Take(gpu(), 10, true),
                step(Input::ActiveChanged(true), []),
                step(resume(gpu()), [ACTIVATED]),
            ],
        ),
        (
            "a device taken while inactive counts as paused until it resumes",
            Machine::new(true),
            vec![
                Take(gpu(), 10, true),
                step(Input::ActiveChanged(false), [PAUSED]),
                step(Input::ActiveChanged(true), []),
                Running(false),
                step(resume(gpu()), [ACTIVATED]),
            ],
        ),
        (
            "a gone device holds nothing back and its later signals are ignored",
            running(),
            vec![
                step(gone(gpu()), []),
                step(pause(gpu()), []),
                step(resume(gpu()), []),
                step(Input::ActiveChanged(false), [PAUSED]),
                step(Input::ActiveChanged(true), [ACTIVATED]),
            ],
        ),
        (
            "signals about a device not held are ignored",
            running(),
            vec![
                step(pause(other()), []),
                step(force(other()), []),
                step(gone(other()), []),
                step(resume(other()), []),
                Running(true),
            ],
        ),
        (
            "a released paused device no longer holds the session back",
            running(),
            vec![
                step(Input::ActiveChanged(false), [PAUSED]),
                step(pause(gpu()), [Output::CompletePause(gpu())]),
                step(Input::ActiveChanged(true), []),
                Running(false),
                step(resume(keyboard()), []),
                Release(10),
                step(Input::Recheck, [ACTIVATED]),
                Running(true),
            ],
        ),
        (
            "a recheck changes nothing while active is false or the session runs",
            running(),
            vec![
                step(Input::ActiveChanged(false), [PAUSED]),
                step(Input::Recheck, []),
                step(Input::ActiveChanged(true), [ACTIVATED]),
                step(Input::Recheck, []),
            ],
        ),
        (
            "a released device does not start a pausing session again",
            running(),
            vec![
                step(pause(gpu()), [PAUSED, Output::CompletePause(gpu())]),
                Release(10),
                step(Input::Recheck, []),
                Running(false),
            ],
        ),
    ];
    for (name, mut machine, steps) in scenarios {
        for (index, item) in steps.into_iter().enumerate() {
            match item {
                Step::Step(input, expected) => {
                    assert_eq!(machine.step(input), expected, "{name}, step {index}");
                }
                Take(dev, fd, inactive) => machine.take(dev, fd, inactive).unwrap(),
                Release(fd) => {
                    machine.release(fd).unwrap();
                }
                Running(running) => {
                    assert_eq!(machine.is_running(), running, "{name}, step {index}");
                }
            }
        }
    }
}

#[test]
fn an_acknowledgement_is_due_once_and_only_while_the_device_is_held_and_asked() {
    let mut machine = running();
    machine.step(pause(gpu()));
    assert!(machine.ack_pause(gpu()));
    assert!(!machine.ack_pause(gpu()), "already answered");
    assert!(!machine.ack_pause(keyboard()), "never asked");
    assert!(!machine.ack_pause(DevT::new(1, 1)), "not held");

    machine.step(pause(keyboard()));
    machine.release(11).unwrap();
    assert!(!machine.ack_pause(keyboard()), "released since");

    let mut machine = running();
    machine.step(force(gpu()));
    assert!(!machine.ack_pause(gpu()), "forced");
}

#[test]
fn a_device_is_held_once_under_the_descriptor_it_was_taken_with() {
    let mut machine = running();
    assert!(matches!(
        machine.take(gpu(), 12, false),
        Err(Error::AlreadyTaken(dev)) if dev == gpu()
    ));
    assert!(matches!(machine.release(99), Err(Error::UnknownFd(99))));
    machine.release(11).unwrap();
    assert!(matches!(machine.release(11), Err(Error::UnknownFd(11))));

    // a device that went away can be taken again; the old descriptor is no longer recognised
    machine.step(gone(gpu()));
    machine.take(gpu(), 20, false).unwrap();
    assert!(matches!(machine.release(10), Err(Error::UnknownFd(10))));
    assert_eq!(machine.release(20).unwrap().dev, gpu());
}

#[test]
fn releasing_asks_logind_to_let_go_unless_the_device_is_gone() {
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

    machine.step(gone(keyboard()));
    assert_eq!(
        machine.release(11).unwrap(),
        Released {
            dev: keyboard(),
            notify_logind: false
        }
    );
    assert!(!machine.is_unreleased(keyboard()));
}
