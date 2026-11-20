//! A shell host built from `cupertino_kit_shell::hosting` against the real server, headless
//! (`docs/architecture/shell-host.md`). The server's supervisor starts this test binary as `cupertino-shell`: with
//! [`host::MARK`] among its arguments it runs the host of `shell-host/host.rs` and the components of
//! `shell-host/components.rs`, each on Kit's UI loop on the connection the server handed it.
//! - A component binds its `cupertino_component_v1`, sees the server's outputs, draws its view, persists its surface
//!   before the surface's first buffer and says ready once it was shown: a restart is answered then, and the new
//!   instance takes the held surface over.
//! - The menu bar and the dock take their roles on the server's own globals, at the versions it offers.
//! - A host killed with its components leaves the menu bar's reserved space and picture held; the host the supervisor
//!   starts again takes the bar over on the frame its new instance draws, the usable area never reflowing.
//! - The overview claims a client's window for a hole of its surface in one transaction: claim and hole show
//!   together, and the window then shows in the hole alone.
//! - A privileged program's `capture::once` of the output returns its pixels.
//! - The host bus's bridge carries publications between the shared host and a separate host (`--only dock`), both
//!   ways.
//! - A busy loop and a busy render thread are reported and seen by the server; a helper that ignores its stop makes
//!   the host ask for its restart, which the server answers at its quiet moment, and the supervisor starts the host
//!   again.

#[path = "shell-host/app.rs"]
mod app;
#[path = "shell-host/components.rs"]
mod components;
#[path = "shell-host/host.rs"]
mod host;
#[path = "shell-host/rig.rs"]
mod rig;
#[path = "shell-host/shot.rs"]
mod shot;

use std::os::fd::IntoRawFd;

use cupertino_kit::render::PixelFormat;
use cupertino_server::host::{HostKind, Liveness};

use components::{bar_height, BAR_KEY, DOCK_SIZE, HELLO_KEY, HELLO_SIZE, HOLE_APP_ID, HOLE_SIZE};
use rig::{count, magenta, primary, Area, Rig, OUTPUT_SIZE};

/// The host itself, when the supervisor started this binary with [`host::MARK`]; a test run does nothing here.
#[test]
fn host_program() {
    if let Some(report) = host::report_file() {
        host::run(report);
    }
}

/// How many pixels of the output a surface of `size` must cover with its colour to be shown.
fn covering(size: cupertino_kit::geometry::Size) -> usize {
    Area::new(0, 0, size.width as i32, size.height as i32).covered()
}

/// The area the menu bar takes: the output's top, as tall as the theme's bar.
fn bar_area() -> Area {
    Area::new(0, 0, OUTPUT_SIZE.0, bar_height() as i32)
}

/// The first mapped window of the server, and its frame on the output.
fn mapped(rig: &mut Rig) -> (cupertino_wm::WindowId, Area) {
    rig.until("the window is mapped", |rig| {
        rig.state.windows.iter().any(|w| w.mapped)
    });
    let id = rig.state.windows.iter().find(|w| w.mapped).unwrap().id;
    let frame = rig.state.wm.window(id).unwrap().geometry();
    let area = Area::new(
        frame.x as i32,
        frame.y as i32,
        frame.w as i32,
        frame.h as i32,
    );
    (id, area)
}

#[test]
fn a_component_draws_its_view_persists_it_and_says_ready() {
    let mut rig = Rig::new("hello", &["osd"], &[]);
    let green = covering(HELLO_SIZE);
    rig.until("the osd draws its window", |rig| {
        rig.state.hosts.component_connected("osd") && rig.shows(Area::output(), primary(1), green)
    });
    assert_eq!(rig.state.hosts.component_instance("osd"), 1);
    let (width, height) = OUTPUT_SIZE;
    rig.until_reported(&format!("osd outputs headless-1:{width}x{height}@1"));
    assert!(
        rig.reported("osd holds "),
        "the first instance holds nothing"
    );

    // the server answers a restart once the new instance said it is ready; that instance takes the surface over
    let rx = rig.restart("osd");
    rig.answer(&rx).expect("the restarted osd says it is ready");
    assert_eq!(rig.state.hosts.component_instance("osd"), 2);
    rig.until_reported(&format!("osd holds {HELLO_KEY}"));
    rig.until("the new instance took the held surface over", |rig| {
        rig.state.hosts.held_surfaces().is_empty()
    });
    assert!(
        rig.shows(Area::output(), primary(1), green),
        "the new instance draws"
    );
}

#[test]
fn the_menu_bar_and_the_dock_take_their_roles_on_the_servers_globals() {
    let mut rig = Rig::new("roles", &["menubar", "dock"], &[]);
    let red = covering(DOCK_SIZE);
    rig.until("the bar and the dock are drawn", |rig| {
        let image = rig.image();
        count(&image, bar_area(), primary(2)) >= bar_area().covered()
            && count(&image, Area::output(), primary(0)) >= red
    });
    assert!(rig.reported("host started menubar") && rig.reported("host started dock"));
    assert!(
        !rig.lines().iter().any(|l| l.starts_with("host version")),
        "the server's globals are as new as the components need: {:?}",
        rig.lines()
    );
    let output = rig.state.outputs[0].id;
    assert!(
        rig.state.menu_bars.has_bar(output),
        "the bar has the menu bar role"
    );
    assert!(
        rig.state.docks.has_dock(output),
        "the dock has the dock role"
    );
    assert_eq!(
        rig.state.usable_area(output).y,
        bar_height(),
        "the bar's space is reserved"
    );
}

#[test]
fn a_held_bar_keeps_its_space_and_picture_until_a_restarted_host_takes_it_over() {
    let mut rig = Rig::new("holds", &["menubar"], &[]);
    let blue = bar_area().covered();
    rig.until("the bar is drawn", |rig| {
        rig.shows(bar_area(), primary(2), blue)
    });
    let output = rig.state.outputs[0].id;
    let first = rig
        .host_pid(HostKind::Shared)
        .expect("the shared host runs");

    // the host goes with its components, as in a crash
    let pid = rustix::process::Pid::from_raw(first as i32).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::KILL).unwrap();
    rig.until("the bar is held", |rig| {
        rig.state
            .hosts
            .held_surfaces()
            .iter()
            .any(|h| h.component == "menubar" && h.key == BAR_KEY && h.picture.is_some())
    });

    // the supervisor starts the host again; its menu bar takes the hold over on its first frame
    rig.until("the new instance took the bar over", |rig| {
        assert_eq!(
            rig.state.usable_area(output).y,
            bar_height(),
            "the bar's space stays reserved"
        );
        assert!(
            rig.shows(bar_area(), primary(2), blue),
            "on every turn the bar shows, held or drawn anew"
        );
        rig.state.hosts.held_surfaces().is_empty()
            && rig.state.hosts.component_instance("menubar") == 2
    });
    let second = rig
        .host_pid(HostKind::Shared)
        .expect("the shared host runs again");
    assert_ne!(first, second, "a new host process");
    assert!(rig.reported(&format!("menubar holds {BAR_KEY}")));
    assert!(rig.state.menu_bars.has_bar(output));
}

#[test]
fn a_transaction_shows_a_hole_claiming_a_window_on_one_frame() {
    let mut rig = Rig::new("hole", &["overview"], &[]);
    rig.until("the overview is connected", |rig| {
        rig.state.hosts.component_connected("overview")
    });
    let client = rig.client();
    app::spawn(client, HOLE_APP_ID);
    let (id, own) = mapped(&mut rig);
    let hole = Area::new(0, 0, HOLE_SIZE.width as i32, HOLE_SIZE.height as i32);
    assert!(
        !own.meets(hole),
        "the window {own:?} lies apart from the hole"
    );

    // the window shows in the hole on the frame the claim starts on, never before and never after without it
    rig.until("the transaction started", |rig| {
        let claimed = rig.state.transactions.is_window_claimed(id);
        let image = rig.image();
        let in_hole = count(&image, hole, magenta) > 0;
        assert_eq!(claimed, in_hole, "the claim and the hole show together");
        rig.reported("overview started") && claimed
    });
    assert!(rig.reported(&format!("overview claims {}", id.0)));
    rig.until("the window shows in its hole alone", |rig| {
        let image = rig.image();
        count(&image, own, magenta) == 0 && count(&image, hole, magenta) > own.covered()
    });
}

#[test]
fn capture_once_of_an_output_returns_its_pixels() {
    let mut rig = Rig::new("capture", &[], &[]);
    let client = rig.client();
    app::spawn(client, "org.cupertino.test.captured");
    let (_, own) = mapped(&mut rig);
    rig.until("the window is drawn", |rig| {
        rig.shows(own, magenta, own.covered())
    });

    // the program runs on a connection with the capture role, as the server starts `cupertino-screenshot`
    let stream = rig.state.privileged_client().unwrap();
    std::env::set_var("WAYLAND_SOCKET", stream.into_raw_fd().to_string());
    let environment = cupertino_kit::app::environment().expect("the toolkit's environment");
    let program = std::thread::spawn(move || cupertino_kit_shell::run::<shot::Shot>(environment));
    rig.until("the frame is taken", |_| {
        shot::TAKEN.lock().unwrap().is_some()
    });
    rig.until("the program exits", |_| program.is_finished());
    program.join().unwrap().expect("the program ran");

    let image = shot::TAKEN
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .expect("a frame");
    assert_eq!((image.width as i32, image.height as i32), OUTPUT_SIZE);
    assert!(matches!(
        image.format,
        PixelFormat::Rgba8 | PixelFormat::Rgba8Premultiplied
    ));
    let taken = image::RgbaImage::from_fn(image.width, image.height, |x, y| {
        let i = (y * image.stride + x * 4) as usize;
        image::Rgba([
            image.pixels[i],
            image.pixels[i + 1],
            image.pixels[i + 2],
            255,
        ])
    });
    let shown = count(&taken, own, magenta);
    assert!(
        shown >= own.covered(),
        "the frame shows the window: {shown} pixels"
    );
}

#[test]
fn the_bus_bridge_carries_publications_between_a_separate_host_and_the_shared_host() {
    let mut rig = Rig::new("bus", &["osd", "dock"], &["dock"]);
    rig.until("both hosts run", |rig| {
        rig.host_pid(HostKind::Shared).is_some()
            && rig.host_pid(HostKind::Separate("dock")).is_some()
    });
    let separate = rig.host_pid(HostKind::Separate("dock")).unwrap();
    let args = std::fs::read(format!("/proc/{separate}/cmdline")).unwrap();
    let args: Vec<String> = args
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    assert!(
        args.ends_with(&["--only".into(), "dock".into()]),
        "{args:?}"
    );
    assert!(rig.host(HostKind::Shared).unwrap().components == vec!["osd"]);
    rig.until_reported("osd heard dock PanelShown");
    rig.until_reported("dock heard osd DoNotDisturbChanged");
}

#[test]
fn a_busy_loop_a_busy_render_thread_and_a_stuck_helper_reach_the_server() {
    let mut rig = Rig::new("busy", &["notifications"], &[]);
    rig.until_reported("notifications started");
    let mut seen = Vec::new();
    let note = |rig: &Rig, seen: &mut Vec<Liveness>| {
        let now = rig.state.hosts.component_liveness("notifications");
        if seen.last() != Some(&now) {
            seen.push(now);
        }
    };
    rig.until(
        "the stalled loop was reported busy and then responsive",
        |rig| {
            note(rig, &mut seen);
            rig.reported("host liveness notifications Busy loop")
                && rig.state.hosts.component_liveness("notifications") == Liveness::Responsive
        },
    );
    rig.until(
        "the stalled render thread was reported busy and then responsive",
        |rig| {
            note(rig, &mut seen);
            rig.reported("host liveness notifications Busy render")
                && rig.state.hosts.component_liveness("notifications") == Liveness::Responsive
        },
    );
    assert_eq!(
        seen,
        [
            Liveness::Responsive,
            Liveness::Busy,
            Liveness::Responsive,
            Liveness::Busy,
            Liveness::Responsive
        ],
        "the server saw each stall; the hosts reported:\n{}",
        rig.lines().join("\n")
    );

    // a restart stops the component; its helper ignores the stop, so the host asks to be restarted itself
    rig.until_reported("notifications helper ignores its stop");
    let first = rig.host_pid(HostKind::Shared).unwrap();
    let _answer = rig.restart("notifications");
    rig.until_reported("host restarts notifications");
    rig.until_reported("host restart");
    rig.until("the supervisor started the host again", |rig| {
        rig.host_pid(HostKind::Shared)
            .is_some_and(|pid| pid != first)
    });
    rig.until("the component runs again", |rig| {
        rig.state.hosts.component_connected("notifications")
            && rig
                .lines()
                .iter()
                .filter(|l| *l == "notifications started")
                .count()
                >= 2
    });
}
