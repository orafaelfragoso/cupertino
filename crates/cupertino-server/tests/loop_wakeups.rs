//! The main loop sleeps until something is due: the work a tick does on its own clock names its next deadline, and
//! no tick leaves one behind that is already due, which would wake the loop over and over for nothing.

mod common;

use common::desktop::Desktop;
use cupertino_server::hung::{PING_INTERVAL, PONG_TIMEOUT};

#[test]
fn after_a_tick_nothing_it_waits_for_is_already_due() {
    let mut d = Desktop::new(1.0);
    d.window();
    for ms in [3000, 3400, 4100, 6000, 9000] {
        d.at(ms);
        let now = d.state.now();
        let next = d
            .state
            .next_tick_deadline()
            .expect("a mapped window is pinged");
        assert!(next > now, "at {ms} ms a deadline of {next} s was left due");
        assert!(
            next <= now + PING_INTERVAL.max(PONG_TIMEOUT),
            "at {ms} ms the next ping or pong timeout is due by {next} s"
        );
    }
}

#[test]
fn an_idle_desktop_has_nothing_due_within_a_second() {
    let mut d = Desktop::new(1.0);
    d.at(3000);
    let now = d.state.now();
    assert!(!d.state.animating());
    assert!(
        d.state
            .next_tick_deadline()
            .is_none_or(|next| next - now >= 1.0),
        "{:?} at {now}",
        d.state.next_tick_deadline()
    );
}
