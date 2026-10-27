//! Keeping `Capabilities` current: logind's answers and the session manager's presence are read on a thread of their own,
//! whenever the menu asks (`RefreshCapabilities`) and when the manager's name gets an owner, so no question to logind (a bus
//! round trip that can wait for a polkit decision) holds the agent's main loop. Many requests in a row are one read.

use std::sync::mpsc::Receiver;

use crate::capabilities::{Capabilities, Inputs};

/// For every request on `requests`, read `read` and hand the result to `publish`, until the sender is dropped. What is off
/// is logged with its reason when the inputs differ from the last read.
pub fn run(
    requests: Receiver<()>,
    mut read: impl FnMut() -> Inputs,
    mut publish: impl FnMut(Capabilities),
) {
    let mut last: Option<Inputs> = None;
    while requests.recv().is_ok() {
        while requests.try_recv().is_ok() {}
        let inputs = read();
        if last != Some(inputs) {
            for reason in Capabilities::reasons(&inputs) {
                eprintln!("cupertino-login: menu item off: {reason}");
            }
            last = Some(inputs);
        }
        publish(Capabilities::new(&inputs));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::Answer;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    fn inputs(can_suspend: Answer) -> Inputs {
        Inputs {
            can_suspend,
            can_reboot: Answer::Yes,
            can_power_off: Answer::Yes,
            session_manager: true,
        }
    }

    #[test]
    fn every_request_publishes_what_logind_says_now_and_a_burst_is_one_read() {
        let (tx, rx) = mpsc::channel();
        let answers = Arc::new(Mutex::new(vec![
            inputs(Answer::Yes),
            inputs(Answer::No),
            inputs(Answer::Yes),
        ]));
        let reads = Arc::new(Mutex::new(0u32));
        let published = Arc::new(Mutex::new(Vec::new()));
        let (a, r, p) = (answers.clone(), reads.clone(), published.clone());
        let thread = std::thread::spawn(move || {
            run(
                rx,
                move || {
                    *r.lock().unwrap() += 1;
                    a.lock().unwrap().remove(0)
                },
                move |caps| p.lock().unwrap().push(caps.sleep),
            )
        });
        // a burst queued before the reader runs: one read
        for _ in 0..5 {
            tx.send(()).unwrap();
        }
        drop(tx);
        thread.join().unwrap();
        assert_eq!(*reads.lock().unwrap(), 1, "five requests, one read");
        assert_eq!(*published.lock().unwrap(), [true]);

        // later requests read again and see the change
        let (tx, rx) = mpsc::channel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (s, a) = (seen.clone(), answers.clone());
        let thread = std::thread::spawn(move || {
            run(
                rx,
                move || a.lock().unwrap().remove(0),
                move |caps| s.lock().unwrap().push(caps.sleep),
            )
        });
        let ready = |count: usize| {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while seen.lock().unwrap().len() < count {
                assert!(std::time::Instant::now() < end, "no publication");
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        };
        tx.send(()).unwrap();
        ready(1);
        tx.send(()).unwrap();
        ready(2);
        drop(tx);
        thread.join().unwrap();
        assert_eq!(*seen.lock().unwrap(), [false, true]);
    }
}
