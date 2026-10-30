//! Session lock against a headless compositor: a toplevel client and a lock client connect through the socket.
//! While the lock holds, only the lock surface may take the pointer, the keyboard and the pixels. Tests skip
//! (pass) when no EGL device is available.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
mod common;

use common::Client;

const OUTPUT: (i32, i32) = (1000, 700);

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("creating the event loop");
        let display: Display<Cupertino> = Display::new().expect("creating the Wayland display");
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("creating the compositor");
        match headless::init(&mut state, OUTPUT, 1.0) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                common::egl_unavailable(format!("{e:#}"));
                None
            }
        }
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    fn screenshot(&mut self, name: &str) -> image::RgbaImage {
        let path =
            std::env::temp_dir().join(format!("cupertino-test-{}-{name}.png", std::process::id()));
        self.state.screenshot_requests.push(path.clone());
        self.state.request_redraw();
        let p = path.clone();
        self.wait_until("the screenshot", move |_| p.exists());
        let img = image::open(&path).unwrap().to_rgba8();
        let _ = std::fs::remove_file(path);
        img
    }

    /// A point inside the only toplevel.
    fn toplevel_center(&self) -> (f64, f64) {
        let id = self.state.wm.key_window().expect("a focused toplevel");
        let g = self.state.wm.window(id).unwrap().geometry();
        (g.x + g.w / 2.0, g.y + g.h / 2.0)
    }

    fn surface_under(
        &self,
        p: (f64, f64),
    ) -> Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> {
        self.state.surface_under(p.into()).map(|(s, _)| s)
    }

    fn keyboard_focus(
        &self,
    ) -> Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> {
        self.state.seat.get_keyboard().unwrap().current_focus()
    }

    fn center_pixel(&mut self, name: &str) -> [u8; 4] {
        let p = self.toplevel_center();
        self.screenshot(name).get_pixel(p.0 as u32, p.1 as u32).0
    }
}

fn mapped_toplevel(h: &mut Harness) -> (Client, (f64, f64)) {
    let client = Client::toplevel(h.socket_path());
    h.wait_until("the toplevel to be mapped", |s| {
        s.wm.windows().count() == 1
            && s.wm
                .key_window()
                .is_some_and(|w| s.entry(w).is_some_and(|e| e.mapped))
    });
    // the opening animation settles
    let time = h.state.now() + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    let center = h.toplevel_center();
    let under = h
        .surface_under(center)
        .expect("the toplevel is under its center");
    assert!(
        h.state.entry_for_surface(&under).is_some(),
        "a toplevel surface is hit before the lock"
    );
    (client, center)
}

#[test]
fn a_lock_covers_the_toplevel_and_gives_the_pointer_and_keyboard_to_the_lock_surface() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    assert_eq!(
        h.center_pixel("before")[..3],
        [0, 0, 255][..],
        "the toplevel shows before the lock"
    );

    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());

    let under = h
        .surface_under(center)
        .expect("the lock surface is under the pointer");
    assert!(
        h.state.is_lock_surface(&under),
        "the surface under the former toplevel is the lock surface"
    );
    assert!(
        h.keyboard_focus()
            .is_some_and(|s| h.state.is_lock_surface(&s)),
        "the keyboard is on the lock surface"
    );
    assert_eq!(
        h.center_pixel("locked")[..3],
        [0, 255, 0][..],
        "only the lock surface is drawn"
    );
}

#[test]
fn a_lock_client_that_disconnects_leaves_the_session_locked_and_black() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    drop(locker);

    h.wait_until("the lock surface to go away", |s| {
        s.surface_under(center.into()).is_none()
    });
    assert!(
        h.state.session_lock.is_active(),
        "the session is still locked"
    );
    assert!(
        h.keyboard_focus().is_none(),
        "no keyboard focus while the lock has no client"
    );
    assert_eq!(
        h.center_pixel("orphaned")[..3],
        [0, 0, 0][..],
        "the output is black"
    );

    let successor = Client::locker(common::session_agent(&h.state));
    h.wait_until("a new client to take the lock", |_| successor.locked());
    let under = h
        .surface_under(center)
        .expect("the new lock surface is under the pointer");
    assert!(h.state.is_lock_surface(&under));
}

#[test]
fn a_second_lock_request_is_refused_while_the_session_is_locked() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, _) = mapped_toplevel(&mut h);
    let holder = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| holder.locked());
    let intruder = Client::locker(common::session_agent(&h.state));
    h.wait_until("the second request to be refused", |_| intruder.finished());
    assert!(!intruder.locked());
}

#[test]
fn an_unlock_from_a_refused_lock_does_not_open_a_session_whose_locker_crashed() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    let holder = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| holder.locked());
    let intruder = Client::locker(common::session_agent(&h.state));
    h.wait_until("the second request to be refused", |_| intruder.finished());

    // both reach the compositor in the same dispatch: the intruder's unlock request and the holder's crash
    intruder.unlock();
    std::thread::sleep(Duration::from_millis(100));
    holder.kill();
    h.wait_until("the holder's lock surface to go away", |s| {
        s.surface_under(center.into()).is_none()
    });
    assert!(
        h.state.session_lock.is_active(),
        "the session is still locked"
    );
    assert_eq!(
        h.center_pixel("after the intruder's unlock")[..3],
        [0, 0, 0][..],
        "the output is black"
    );
}

#[test]
fn unlocking_gives_the_toplevel_back_the_pointer_the_keyboard_and_the_screen() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    let toplevel_surface = h.surface_under(center).unwrap();
    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());

    locker.unlock();
    h.wait_until("the session to be unlocked", |s| {
        !s.session_lock.is_active()
    });
    assert_eq!(
        h.surface_under(center),
        Some(toplevel_surface.clone()),
        "the toplevel is under the pointer again"
    );
    assert_eq!(
        h.keyboard_focus(),
        Some(toplevel_surface),
        "the toplevel has the keyboard again"
    );
    assert_eq!(
        h.center_pixel("unlocked")[..3],
        [0, 0, 255][..],
        "the toplevel is drawn again"
    );
}

/// While the session is locked an application's layer surface cannot take the keyboard exclusively, neither to read
/// what is typed at the lock screen nor to be waiting for it at the unlock.
#[test]
fn an_application_cannot_take_the_keyboard_exclusively_while_locked() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, _) = mapped_toplevel(&mut h);
    let surface = {
        let window = h.state.wm.key_window().unwrap();
        h.state.entry(window).unwrap().surface().clone()
    };

    h.state.layer_keyboard_exclusivity(&surface, true);
    assert_eq!(
        h.state.exclusive_layer.as_ref(),
        Some(&surface),
        "outside a lock a layer surface takes the keyboard"
    );
    h.state.layer_keyboard_exclusivity(&surface, false);

    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    h.state.layer_keyboard_exclusivity(&surface, true);
    assert_eq!(
        h.state.exclusive_layer, None,
        "an application took the keyboard at the lock screen"
    );
}

/// An application is told the session is locking before the lock client is told it holds the lock, and unlocked only
/// once the lock is over; one that starts during the lock is told at once, and one bound below version 5 never is.
#[test]
fn an_application_learns_the_lock_as_it_starts_and_the_unlock_once_it_is_over() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, _) = mapped_toplevel(&mut h);
    let current = Client::appearance_observer(h.socket_path(), 5);
    let older = Client::appearance_observer(h.socket_path(), 4);
    h.wait_until("the observer to be told the session is open", |_| {
        current.lock_events() == ["session_lock 0"]
    });

    let locker = Client::observing_locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    assert_eq!(
        locker.lock_events(),
        ["session_lock 0", "session_lock 1", "locked"]
    );
    h.wait_until("the observer to be told the session is locked", |_| {
        current.lock_events().len() == 2
    });
    let late = Client::appearance_observer(h.socket_path(), 5);
    h.wait_until("the late observer to be told", |_| {
        !late.lock_events().is_empty()
    });
    assert_eq!(late.lock_events(), ["session_lock 1"]);

    locker.unlock();
    h.wait_until("the observers to be told the session is open again", |_| {
        current.lock_events().len() == 3 && late.lock_events().len() == 2
    });
    assert!(!h.state.session_lock.is_active());
    assert_eq!(
        current.lock_events(),
        ["session_lock 0", "session_lock 1", "session_lock 0"]
    );
    assert_eq!(late.lock_events(), ["session_lock 1", "session_lock 0"]);
    assert!(
        older.lock_events().is_empty(),
        "a version 4 observer was told"
    );
}

#[test]
fn an_application_stays_told_locked_when_the_lock_client_crashes() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    let observer = Client::appearance_observer(h.socket_path(), 5);
    h.wait_until("the observer to be told the session is open", |_| {
        !observer.lock_events().is_empty()
    });
    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    locker.kill();
    h.wait_until("the lock surface to go away", |s| {
        s.surface_under(center.into()).is_none()
    });
    let successor = Client::locker(common::session_agent(&h.state));
    h.wait_until("a new client to take the lock", |_| successor.locked());
    assert_eq!(observer.lock_events(), ["session_lock 0", "session_lock 1"]);
}
