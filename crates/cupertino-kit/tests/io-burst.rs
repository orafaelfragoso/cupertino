//! `kit::task`: a burst of I/O tasks handed over while one of the pool's threads waits for a job starts a thread for
//! each, up to the instance's share, instead of queueing them behind the one that waits.

use crate::support;

use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::task::{self, IO_SHARE};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// How many tasks wait at the gate, and whether it is open.
static GATE: (Mutex<(usize, bool)>, Condvar) = (Mutex::new((0, false)), Condvar::new());

/// Waits at the gate until it opens, as a look-up on a mount that does not answer.
fn held() {
    let (gate, changed) = &GATE;
    let mut state = gate.lock().unwrap();
    state.0 += 1;
    changed.notify_all();
    while !state.1 {
        state = changed.wait(state).unwrap();
    }
}

/// Runs one quick I/O task, which leaves a pool thread waiting for a job, then hands over its share of held tasks at
/// once.
struct Burst;

#[derive(Debug, Clone)]
enum Message {
    Warm,
    Held,
}

impl Application for Burst {
    /// Whether the burst was handed over, and how many of its tasks finished.
    type Model = (bool, usize);
    type Message = Message;
    const RESIDENT: bool = true;

    fn init() -> ((bool, usize), Vec<Command<Message>>) {
        (
            (false, 0),
            vec![task::io(Duration::from_secs(60), || (), |()| Message::Warm)],
        )
    }

    fn update(model: &mut (bool, usize), message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Warm => {
                // the warm thread has handed its result over and goes back to wait for a job
                std::thread::sleep(Duration::from_millis(50));
                model.0 = true;
                (0..IO_SHARE)
                    .map(|_| task::io(Duration::from_secs(60), held, |()| Message::Held))
                    .collect()
            }
            Message::Held => {
                model.1 += 1;
                Vec::new()
            }
        }
    }

    fn view(_: &(bool, usize)) -> Vec<Window<Message>> {
        Vec::new()
    }
}

#[test]
fn a_burst_of_io_tasks_handed_over_while_a_thread_waits_all_start() {
    let mut burst = support::driver::<Burst>(support::text_system());
    support::run_until(&mut burst, |(handed, _)| *handed);
    let (gate, changed) = &GATE;
    {
        let state = gate.lock().unwrap();
        let (state, waited) = changed
            .wait_timeout_while(state, support::PATIENCE, |s| s.0 < IO_SHARE)
            .unwrap();
        assert!(!waited.timed_out(), "{} of {IO_SHARE} started", state.0);
    }
    gate.lock().unwrap().1 = true;
    changed.notify_all();
    support::run_until(&mut burst, |(_, finished)| *finished == IO_SHARE);
}
