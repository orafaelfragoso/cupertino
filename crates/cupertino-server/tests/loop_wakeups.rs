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

/// What `Cupertino::tick` calls, each audited for how the loop wakes for it (Shared/cupertino/prism/row236-tick-audit.md):
/// an event, a frame (`animating`), or a deadline in `next_tick_deadline`. A call added to the tick fails this test until
/// it is audited the same way and listed here; otherwise its time-driven work would run up to a second late.
const AUDITED: [&str; 25] = [
    "crate::acquire_waits::sweep(",
    "self.apply_effects(",
    // a drag that ended is an event: the drop handler leaves it, and the tick only finishes what it left
    "self.drag_ended(",
    "self.forget_gone_processes(",
    "self.fx.snapshot_age(",
    "self.fx.tick(",
    "self.idle.refresh_inhibition(",
    "self.now(",
    // the cursor, a drag icon and an input method's popup move with events (pointer motion, a new cursor, a drag, the
    // text cursor, an output change), and the cursor's shake growth animates, which draws frames
    "self.refresh_loose_surface_outputs(",
    "self.refresh_pointer_constraints(",
    "self.refresh_shortcut_inhibitors(",
    "self.refresh_snapshot(",
    "self.request_redraw(",
    "self.seat.get_keyboard(",
    "self.send_deferred_configures(",
    // the window buttons for assistive technology follow the windows, which change on events and frames; nothing in it
    // runs on a clock
    "self.sync_button_access(",
    "self.tick_display_power(",
    "self.tick_force_quit(",
    "self.tick_hung(",
    "self.tick_input_timers(",
    "self.tick_menu_bars(",
    "self.tick_overlays(",
    "self.tick_quit(",
    "self.tick_session_lock(",
    "self.tick_window_managers(",
    "self.wm.tick(",
];

#[test]
fn every_call_of_the_tick_is_audited_for_how_the_loop_wakes_for_it() {
    let source = include_str!("../src/state.rs");
    let start = source
        .find("    pub fn tick(&mut self) {")
        .expect("Cupertino::tick");
    let body = &source[start..];
    let body = &body[..body.find("\n    }\n").expect("the end of tick")];
    let mut calls: Vec<&str> = Vec::new();
    for (at, _) in body.match_indices('(') {
        let name_start = body[..at]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || "_.:".contains(c)))
            .map_or(0, |i| i + 1);
        let call = &body[name_start..=at];
        if call.starts_with("self.") || call.starts_with("crate::") {
            calls.push(call);
        }
    }
    calls.sort_unstable();
    calls.dedup();
    let unaudited: Vec<&str> = calls
        .iter()
        .copied()
        .filter(|c| !AUDITED.contains(c))
        .collect();
    assert!(
        unaudited.is_empty(),
        "audit these tick calls for a deadline: {unaudited:?}"
    );
}
