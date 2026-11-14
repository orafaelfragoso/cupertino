//! The main loop sleeps until something is due: the work a tick does on its own clock names its next deadline, and
//! no tick leaves one behind that is already due, which would wake the loop over and over for nothing.

use crate::common;

use common::desktop::Desktop;
use cupertino_server::hung::{PING_INTERVAL, PONG_TIMEOUT};

#[test]
fn after_a_tick_nothing_it_waits_for_is_already_due() {
    let mut d = Desktop::new(1.0);
    d.window();
    for ms in [3000, 3400, 4100, 6000, 9000] {
        d.at(ms);
        // the person uses the desktop, so its window is pinged: the input wakes the loop, which ticks
        d.state.input_came();
        d.state.tick();
        let now = d.state.now();
        let next = d
            .state
            .next_tick_deadline()
            .expect("a mapped window is pinged while the desktop is in use");
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

/// What `Cupertino::tick` calls, each audited for how the loop wakes for it: an event, a frame (`animating`), or a
/// deadline in `next_tick_deadline` ([`DEADLINES`] holds every deadline of the tick's clock and where it is read). A call added to the tick fails this test until
/// it is audited the same way and listed here; otherwise its time-driven work would run up to a minute late (the loop's backstop).
const AUDITED: [&str; 29] = [
    "crate::acquire_waits::sweep(",
    "self.apply_effects(",
    // a drag that ended is an event: the drop handler leaves it, and the tick only finishes what it left
    "self.drag_ended(",
    // publications arrive as client requests, which wake the loop; the tick only reads the newest one each sent
    "self.flush_publications(",
    "self.forget_gone_processes(",
    "self.fx.snapshot_age(",
    "self.fx.tick(",
    "self.idle.refresh_inhibition(",
    "self.now(",
    // the cursor, a drag icon and an input method's popup move with events (pointer motion, a new cursor, a drag, the
    // text cursor, an output change), and the cursor's shake growth animates, which draws frames
    // grant lifetime, roles and geometry follow events/frames; pending native replies wake at control_deadline
    "self.refresh_control(",
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
    // a waiting or running layer animation makes the layers want a tick, which draws a frame per refresh
    "self.tick_layers(",
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

/// Every method of the tick's clock that says when its owner next has work (a `fn ...deadline(&self..) -> Option<f64>`
/// or `Option<u64>` in the sources the server ticks), with where it is read, from its file to the loop's wait: `file`,
/// the source holding `read`, the call that reads it. A read in `next_tick_deadline` (`file` is `WAIT`) or in the
/// touchpad's timer feeds the loop; any other read is itself a deadline listed here. A deadline added to a component
/// the tick drives fails this test until it is listed, and a listed one fails when nothing reads it any more.
const DEADLINES: [(&str, &str, &str, &str); 15] = [
    (
        "input/src/edge.rs",
        "next_deadline",
        WAIT,
        "self.bar_edge.next_deadline()",
    ),
    (
        "input/src/hotcorner.rs",
        "next_deadline",
        WAIT,
        "self.hot_corners.next_deadline()",
    ),
    // the touchpad has a timer of its own, armed for the earliest of its devices' deadlines
    (
        "input/src/touchpad.rs",
        "next_deadline",
        "server/src/backend/input.rs",
        "pad.model.next_deadline()",
    ),
    (
        "server/src/backend/input.rs",
        "next_deadline",
        "server/src/backend/input.rs",
        ".filter_map(|opened| opened.translator.next_deadline())",
    ),
    (
        "keyboard/src/mouse_keys.rs",
        "deadline",
        WAIT,
        "filters.mouse.deadline()",
    ),
    (
        "keyboard/src/repeat.rs",
        "next_deadline",
        WAIT,
        "filters.repeat.next_deadline()",
    ),
    (
        "keyboard/src/slow_keys.rs",
        "deadline",
        WAIT,
        "filters.slow.as_ref().and_then(|s| s.deadline())",
    ),
    (
        "server/src/hung.rs",
        "next_deadline",
        WAIT,
        "self.hung.next_deadline()",
    ),
    (
        "server/src/menu_bar.rs",
        "edit_deadline",
        WAIT,
        "self.menu_bars.edit_deadline()",
    ),
    (
        "server/src/display_power.rs",
        "next_deadline",
        "server/src/power.rs",
        "self.power.policy.next_deadline(",
    ),
    (
        "server/src/power.rs",
        "display_power_deadline",
        WAIT,
        "self.display_power_deadline()",
    ),
    (
        "server/src/force_quit.rs",
        "next_deadline",
        WAIT,
        "self.force_quits.next_deadline()",
    ),
    (
        "server/src/control/semantics.rs",
        "control_deadline",
        WAIT,
        "self.control_deadline()",
    ),
    (
        "server/src/session_lock.rs",
        "lock_wait_deadline",
        WAIT,
        "self.lock_wait_deadline()",
    ),
    (
        "wm/src/menu_bar.rs",
        "next_deadline",
        WAIT,
        "self.menu_bars.set.next_deadline()",
    ),
];

/// The body of `next_tick_deadline`, what the loop's wait reads.
const WAIT: &str = "next_tick_deadline";

/// The crates whose clocked work the server's tick drives, under `crates/cupertino-`.
const TICKED: [&str; 4] = ["server", "input", "keyboard", "wm"];

fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The deadline methods of `text`: `fn NAME(&self ...) -> Option<f64>` (or `u64`) with a name ending in `deadline`.
fn deadline_methods(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in text.lines() {
        let Some(at) = line.find("fn ") else { continue };
        let rest = &line[at + 3..];
        let Some(open) = rest.find('(') else { continue };
        let name = &rest[..open];
        let returns_clock = rest.contains("-> Option<f64>") || rest.contains("-> Option<u64>");
        if name.ends_with("deadline")
            && name != WAIT
            && rest[open..].starts_with("(&self")
            && returns_clock
        {
            found.push(name.to_owned());
        }
    }
    found
}

#[test]
fn every_deadline_of_the_ticks_clock_feeds_the_loops_wait() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let read = |rel: &str| {
        let (krate, path) = rel.split_once('/').unwrap();
        std::fs::read_to_string(crates.join(format!("cupertino-{krate}")).join(path)).unwrap()
    };
    let state = read("server/src/state.rs");
    let start = state
        .find("    pub fn next_tick_deadline(&self) -> Option<f64> {")
        .expect("Cupertino::next_tick_deadline");
    let wait = &state[start..];
    let wait = &wait[..wait
        .find("\n    }\n")
        .expect("the end of next_tick_deadline")];
    let mut found: Vec<(String, String)> = Vec::new();
    for krate in TICKED {
        let root = crates.join(format!("cupertino-{krate}"));
        let mut files = Vec::new();
        sources(&root.join("src"), &mut files);
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            let rel = file
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            for name in deadline_methods(&text) {
                found.push((format!("{krate}/{rel}"), name));
            }
        }
    }
    found.sort();
    found.dedup();
    let mut listed: Vec<(String, String)> = DEADLINES
        .iter()
        .map(|(file, name, ..)| ((*file).to_owned(), (*name).to_owned()))
        .collect();
    listed.sort();
    assert_eq!(
        found, listed,
        "a deadline of the tick's clock is missing from DEADLINES, or a listed one is gone"
    );
    for (file, name, reader, call) in DEADLINES {
        let fed = if reader == WAIT {
            wait.contains(call)
        } else {
            read(reader).contains(call)
        };
        assert!(
            fed,
            "{file} `{name}` is not read where listed: `{call}` in {reader}"
        );
    }
}

#[test]
fn a_deadline_method_is_found_by_its_name_receiver_and_clock() {
    let text = "    pub fn next_deadline(&self) -> Option<f64> {\n\
                \x20   fn deadline(&self, now: f64) -> Option<u64> {\n\
                \x20   fn arm_deadline(&mut self) {\n\
                \x20   pub fn frame_deadline(&self) -> Option<Duration> {\n\
                \x20   fn next_tick_deadline(&self) -> Option<f64> {\n";
    assert_eq!(deadline_methods(text), ["next_deadline", "deadline"]);
}
